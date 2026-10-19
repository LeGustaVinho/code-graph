using Shop.Data;
using Shop.Handlers;
using Shop.Services;

var builder = WebApplication.CreateBuilder(args);

var connectionString = builder.Configuration.GetConnectionString("Default");
var jwtKey = builder.Configuration["Jwt:Key"];
var region = Environment.GetEnvironmentVariable("SHOP_REGION") ?? "us-east-1";

builder.Services.AddDbContext<AppDbContext>();
builder.Services.AddScoped<IOrderService, OrderService>();
builder.Services.AddSingleton<IClock, SystemClock>();
builder.Services.AddTransient<Dispatcher>();

var app = builder.Build();

app.UseExceptionHandler("/error");

app.MapGet("/api/health", () => Results.Ok("healthy"));

app.Run();
